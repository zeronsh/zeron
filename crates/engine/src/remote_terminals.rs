//! Terminals on other devices: the hub lives in `zeron-term-link` (the
//! phone runs the same one); this engine supplies its relay and its direct
//! peers.

pub use zeron_term_link::remote::*;

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use base64::Engine as _;
    use std::sync::atomic::AtomicBool;

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use zeron_term_link::remote::TerminalPath;

    use async_trait::async_trait;
    use futures::StreamExt;
    use futures::stream::BoxStream;
    use zeron_preview::mux::BoxIo;
    use zeron_proto::TerminalEvent;
    use zeron_rpc::{RpcError, methods};

    use crate::terminals::Terminals;
    use crate::terminals::link;

    fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        mutex
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// One remote device: real PTYs behind a switchable direct path and an
    /// always-up relay, counting which path carried each write.
    struct Host {
        terminals: Terminals,
        direct_up: AtomicBool,
        changed: tokio::sync::Notify,
        links: Mutex<Vec<tokio::task::JoinHandle<()>>>,
        relay_writes: AtomicUsize,
        /// The next direct link accepts the attach, then goes dark: input
        /// and pings vanish, as on a network that just died.
        black_hole: AtomicBool,
    }

    impl Host {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                terminals: Terminals::new(),
                direct_up: AtomicBool::new(false),
                changed: tokio::sync::Notify::new(),
                links: Mutex::new(Vec::new()),
                relay_writes: AtomicUsize::new(0),
                black_hole: AtomicBool::new(false),
            })
        }
        fn set_direct(&self, up: bool) {
            self.direct_up.store(up, Ordering::SeqCst);
            self.changed.notify_waiters();
        }
        /// Sever every live direct link (a network change mid-session).
        fn cut_links(&self) {
            for link in lock(&self.links).drain(..) {
                link.abort();
            }
        }
    }

    #[async_trait]
    impl Direct for Host {
        async fn ready(&self, _device: &str) -> bool {
            self.direct_up.load(Ordering::SeqCst)
        }
        async fn open(&self, _device: &str, service: &str) -> anyhow::Result<BoxIo> {
            // Pairing: wait until the path exists.
            loop {
                let changed = self.changed.notified();
                if self.direct_up.load(Ordering::SeqCst) {
                    break;
                }
                changed.await;
            }
            let id = service
                .strip_prefix(link::SERVICE_PREFIX)
                .unwrap()
                .to_owned();
            let (viewer, host) = tokio::io::duplex(64 * 1024);
            if self.black_hole.swap(false, Ordering::SeqCst) {
                lock(&self.links).push(tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let (mut read, mut write) = tokio::io::split(host);
                    // Answer the attach's ping (kind 19 = PONG), then nothing.
                    let _ = write
                        .write_all(&[19, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0])
                        .await;
                    let mut sink = vec![0; 4096];
                    while matches!(read.read(&mut sink).await, Ok(n) if n > 0) {}
                }));
                return Ok(Box::new(viewer));
            }
            lock(&self.links).push(tokio::spawn(link::serve(self.terminals.clone(), id, host)));
            Ok(Box::new(viewer))
        }
    }

    #[async_trait]
    impl Relay for Host {
        async fn call(
            &self,
            _target: &str,
            method: &str,
            params: serde_json::Value,
        ) -> Result<serde_json::Value, RpcError> {
            let id = params["terminalId"].as_str().unwrap();
            let failed = |e: crate::EngineError| RpcError::Failed(e.to_string());
            match method {
                methods::WRITE_TERMINAL => {
                    self.relay_writes.fetch_add(1, Ordering::SeqCst);
                    self.terminals
                        .write(id, params["data"].as_str().unwrap())
                        .map_err(failed)?
                }
                methods::RESIZE_TERMINAL => self
                    .terminals
                    .resize(
                        id,
                        params["cols"].as_u64().unwrap() as u16,
                        params["rows"].as_u64().unwrap() as u16,
                    )
                    .map_err(failed)?,
                _ => unreachable!("{method}"),
            }
            Ok(serde_json::json!({ "ok": true }))
        }
        async fn subscribe(
            &self,
            _target: &str,
            _method: &str,
            params: serde_json::Value,
        ) -> Result<BoxStream<'static, serde_json::Value>, RpcError> {
            let rx = self
                .terminals
                .subscribe(
                    params["terminalId"].as_str().unwrap(),
                    params["afterSeq"].as_u64(),
                )
                .map_err(|e| RpcError::Failed(e.to_string()))?;
            Ok(futures::stream::unfold(rx, |mut rx| async move {
                let event = rx.recv().await?;
                Some((serde_json::to_value(event).unwrap(), rx))
            })
            .boxed())
        }
    }

    struct Viewer {
        hub: RemoteTerminals,
        host: Arc<Host>,
        id: String,
        stream: BoxStream<'static, serde_json::Value>,
        seqs: Vec<u64>,
        text: String,
    }

    impl Viewer {
        async fn open(host: &Arc<Host>) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let session = host
                .terminals
                .open_with_shell(dir.path().to_str().unwrap(), 80, 24, Some("/bin/sh"))
                .unwrap();
            let hub = RemoteTerminals::default();
            let stream = hub
                .subscribe(
                    host.clone(),
                    Some(host.clone() as Arc<dyn Direct>),
                    "host",
                    &session.id,
                    0,
                    true,
                )
                .await
                .unwrap();
            Self {
                hub,
                host: host.clone(),
                id: session.id,
                stream,
                seqs: Vec::new(),
                text: String::new(),
            }
        }
        fn type_line(&self, line: &str) {
            let relay: Arc<dyn Relay> = self.host.clone();
            self.hub
                .send(
                    &relay,
                    "host",
                    &self.id,
                    Write::Input(line.as_bytes().to_vec()),
                )
                .unwrap();
        }
        async fn until(&mut self, needle: &str) {
            tokio::time::timeout(Duration::from_secs(15), async {
                while !self.text.contains(needle) {
                    let value = self.stream.next().await.expect("stream ended");
                    match serde_json::from_value::<TerminalEvent>(value).unwrap() {
                        TerminalEvent::Data { seq, data } => {
                            self.seqs.push(seq);
                            self.text.push_str(&String::from_utf8_lossy(
                                &base64::engine::general_purpose::STANDARD
                                    .decode(data)
                                    .unwrap(),
                            ));
                        }
                        TerminalEvent::Exit { seq, .. } => self.seqs.push(seq),
                    }
                }
            })
            .await
            .unwrap_or_else(|_| panic!("never saw {needle:?}; saw {:?}", self.text));
        }
        fn assert_contiguous(&self) {
            for pair in self.seqs.windows(2) {
                assert_eq!(pair[1], pair[0] + 1, "gap or repeat in {:?}", self.seqs);
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_open_direct_path_carries_everything() {
        let host = Host::new();
        host.set_direct(true);
        let mut viewer = Viewer::open(&host).await;
        viewer.type_line("echo d''irect-$((1+1))\n");
        viewer.until("direct-2").await;
        assert_eq!(host.relay_writes.load(Ordering::SeqCst), 0);
        viewer.assert_contiguous();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_relayed_terminal_moves_to_the_direct_path_without_a_gap() {
        let host = Host::new();
        let mut viewer = Viewer::open(&host).await;
        viewer.type_line("echo re''lay-$((2+2))\n");
        viewer.until("relay-4").await;
        assert!(host.relay_writes.load(Ordering::SeqCst) > 0);

        host.set_direct(true);
        // The upgrade lands between relay events; wait until writes take it.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !viewer.hub.direct_attached("host", &viewer.id) {
            assert!(tokio::time::Instant::now() < deadline, "never upgraded");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let relayed = host.relay_writes.load(Ordering::SeqCst);
        viewer.type_line("echo up''graded-$((3+3))\n");
        viewer.until("upgraded-6").await;
        assert_eq!(
            host.relay_writes.load(Ordering::SeqCst),
            relayed,
            "writes after the upgrade must take the direct path"
        );
        viewer.assert_contiguous();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_lost_direct_path_falls_back_to_the_relay_mid_stream() {
        let host = Host::new();
        host.set_direct(true);
        let mut viewer = Viewer::open(&host).await;
        viewer.type_line("echo be''fore\n");
        viewer.until("before").await;

        host.set_direct(false);
        host.cut_links();
        // Writes queue on the relay once the link is gone; the stream itself
        // carries on without the viewer resubscribing.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while viewer.hub.direct_attached("host", &viewer.id) {
            assert!(tokio::time::Instant::now() < deadline, "loss not noticed");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        viewer.type_line("echo af''ter-$((4+4))\n");
        viewer.until("after-8").await;
        assert!(host.relay_writes.load(Ordering::SeqCst) > 0);
        viewer.assert_contiguous();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn keys_typed_into_a_dying_link_arrive_once_over_the_relay() {
        let host = Host::new();
        host.set_direct(true);
        host.black_hole.store(true, Ordering::SeqCst);
        let mut viewer = Viewer::open(&host).await;
        host.set_direct(false);
        // Typed into the dead link: nothing acknowledges it.
        viewer.type_line("echo sw''allowed-$((5+5))\n");
        // Silence detection drops the link, the relay takes over and
        // replays the unacknowledged input.
        viewer.until("swallowed-10").await;
        viewer.type_line("echo ne''xt\n");
        viewer.until("next").await;
        assert_eq!(
            viewer.text.matches("swallowed-10").count(),
            1,
            "replayed input must run exactly once: {:?}",
            viewer.text
        );
        viewer.assert_contiguous();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn writes_to_an_ended_terminal_fail() {
        let host = Host::new();
        host.set_direct(true);
        let mut viewer = Viewer::open(&host).await;
        viewer.type_line("exit 0\n");
        tokio::time::timeout(Duration::from_secs(10), async {
            while viewer.stream.next().await.is_some() {}
        })
        .await
        .expect("stream ends after exit");
        let relay: Arc<dyn Relay> = host.clone();
        assert!(
            viewer
                .hub
                .send(&relay, "host", &viewer.id, Write::Input(b"x".to_vec()))
                .is_err()
        );
    }

    /// The phone's side of a real WebRTC pair: a viewer's peers that serve
    /// nothing, whose network can be "changed" (every connection dropped).
    struct PeerDirect {
        peers: zeron_preview::peer::Peers,
        changes: tokio::sync::watch::Sender<u64>,
    }

    struct Refuse;

    #[async_trait]
    impl zeron_preview::mux::Connector for Refuse {
        async fn connect(&self, _service: &str) -> anyhow::Result<BoxIo> {
            anyhow::bail!("viewers serve nothing")
        }
    }

    #[async_trait]
    impl Direct for PeerDirect {
        async fn ready(&self, device: &str) -> bool {
            self.peers.ready(device).await.is_some()
        }
        async fn open(&self, device: &str, service: &str) -> anyhow::Result<BoxIo> {
            Ok(Box::new(self.peers.open(device, service, false).await?))
        }
        async fn changed(&self) {
            let mut changes = self.changes.subscribe();
            let _ = changes.changed().await;
        }
    }

    /// The whole direct path for real: a terminal on a host reached over an
    /// actual WebRTC pair (host candidates on loopback), moving onto the
    /// direct link, back to the relay when the viewer's network changes, and
    /// onto a fresh direct link at the next wake — every keystroke run once,
    /// every output seq delivered once, in order.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_terminal_rides_a_real_webrtc_pair_through_a_network_change() {
        let host = Host::new();
        let stop = tokio_util::sync::CancellationToken::new();
        let (mut host_peers, mut host_out) = zeron_preview::peer::Peers::new(
            "host".into(),
            Arc::new(link::Service::new(host.terminals.clone())),
            stop.clone(),
        );
        let (mut phone_peers, mut phone_out) =
            zeron_preview::peer::Peers::new("phone".into(), Arc::new(Refuse), stop.clone());
        host_peers.set_ice_servers(Vec::new()).unwrap();
        phone_peers.set_ice_servers(Vec::new()).unwrap();
        host_peers
            .set_udp_addrs(vec!["127.0.0.1:0".into()])
            .unwrap();
        phone_peers
            .set_udp_addrs(vec!["127.0.0.1:0".into()])
            .unwrap();
        // Signaling, in process: each side's messages reach the other.
        let to_phone = phone_peers.clone();
        let forward_host = tokio::spawn(async move {
            while let Some(message) = host_out.recv().await {
                let _ = to_phone.signal("host", message.signal).await;
            }
        });
        let to_host = host_peers.clone();
        let forward_phone = tokio::spawn(async move {
            while let Some(message) = phone_out.recv().await {
                let _ = to_host.signal("phone", message.signal).await;
            }
        });
        let direct = Arc::new(PeerDirect {
            peers: phone_peers.clone(),
            changes: tokio::sync::watch::channel(0).0,
        });

        let dir = tempfile::tempdir().unwrap();
        let session = host
            .terminals
            .open_with_shell(dir.path().to_str().unwrap(), 80, 24, Some("/bin/sh"))
            .unwrap();
        let hub = RemoteTerminals::default();
        let relay: Arc<dyn Relay> = host.clone();
        let (stream, mut path) = hub
            .subscribe_tracked(
                relay.clone(),
                Some(direct.clone() as Arc<dyn Direct>),
                "host",
                &session.id,
                0,
                true,
            )
            .await
            .unwrap();
        let mut viewer = Viewer {
            hub: hub.clone(),
            host: host.clone(),
            id: session.id.clone(),
            stream,
            seqs: Vec::new(),
            text: String::new(),
        };
        let wait_path = |path: &mut tokio::sync::watch::Receiver<TerminalPath>,
                         want: TerminalPath| {
            let mut path = path.clone();
            async move {
                tokio::time::timeout(Duration::from_secs(15), path.wait_for(|p| *p == want))
                    .await
                    .unwrap_or_else(|_| panic!("never reached {want:?}"))
                    .unwrap();
            }
        };

        // Not paired yet: the relay starts it, pairing upgrades it.
        wait_path(&mut path, TerminalPath::Direct).await;
        let relayed = host.relay_writes.load(Ordering::SeqCst);
        let started = std::time::Instant::now();
        viewer.type_line("echo pe''er-$((1+1))\n");
        viewer.until("peer-2").await;
        assert_eq!(
            host.relay_writes.load(Ordering::SeqCst),
            relayed,
            "typing rides the WebRTC link"
        );
        assert!(started.elapsed() < Duration::from_secs(2));

        // The phone's network changes: its connections are gone. The stream
        // carries on over the relay, then the wake re-pairs it directly.
        phone_peers.clear().await;
        wait_path(&mut path, TerminalPath::Relay).await;
        viewer.type_line("echo du''ring-$((2+2))\n");
        viewer.until("during-4").await;
        direct.changes.send_modify(|n| *n += 1);
        wait_path(&mut path, TerminalPath::Direct).await;
        viewer.type_line("echo ag''ain-$((3+3))\n");
        viewer.until("again-6").await;

        for marker in ["peer-2", "during-4", "again-6"] {
            assert_eq!(
                viewer.text.matches(marker).count(),
                1,
                "{marker} ran once: {:?}",
                viewer.text
            );
        }
        viewer.assert_contiguous();
        stop.cancel();
        host_peers.clear().await;
        phone_peers.clear().await;
        forward_host.abort();
        forward_phone.abort();
    }

    /// Keystroke → echo through the whole hub (link framing, the host's PTY
    /// and batcher, a real shell): direct over an actual WebRTC pair
    /// (loopback), and over the in-process relay.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "benchmark"]
    async fn bench_keystroke_echo() {
        let host = Host::new();
        let stop = tokio_util::sync::CancellationToken::new();
        let (mut host_peers, mut host_out) = zeron_preview::peer::Peers::new(
            "host".into(),
            Arc::new(link::Service::new(host.terminals.clone())),
            stop.clone(),
        );
        let (mut phone_peers, mut phone_out) =
            zeron_preview::peer::Peers::new("phone".into(), Arc::new(Refuse), stop.clone());
        for peers in [&mut host_peers, &mut phone_peers] {
            peers.set_ice_servers(Vec::new()).unwrap();
            peers.set_udp_addrs(vec!["127.0.0.1:0".into()]).unwrap();
        }
        let to_phone = phone_peers.clone();
        tokio::spawn(async move {
            while let Some(m) = host_out.recv().await {
                let _ = to_phone.signal("host", m.signal).await;
            }
        });
        let to_host = host_peers.clone();
        tokio::spawn(async move {
            while let Some(m) = phone_out.recv().await {
                let _ = to_host.signal("phone", m.signal).await;
            }
        });
        let direct = Arc::new(PeerDirect {
            peers: phone_peers.clone(),
            changes: tokio::sync::watch::channel(0).0,
        });
        for (label, path) in [
            ("relay", None),
            ("direct", Some(direct.clone() as Arc<dyn Direct>)),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let session = host
                .terminals
                .open_with_shell(dir.path().to_str().unwrap(), 80, 24, Some("/bin/sh"))
                .unwrap();
            let hub = RemoteTerminals::default();
            let relay: Arc<dyn Relay> = host.clone();
            let (stream, mut rx) = hub
                .subscribe_tracked(relay.clone(), path.clone(), "host", &session.id, 0, true)
                .await
                .unwrap();
            if path.is_some() {
                tokio::time::timeout(
                    Duration::from_secs(15),
                    rx.wait_for(|p| *p == TerminalPath::Direct),
                )
                .await
                .unwrap()
                .unwrap();
            }
            let mut viewer = Viewer {
                hub: hub.clone(),
                host: host.clone(),
                id: session.id.clone(),
                stream,
                seqs: Vec::new(),
                text: String::new(),
            };
            viewer.type_line("echo ready\n");
            viewer.until("ready").await;
            tokio::time::sleep(Duration::from_millis(300)).await;
            let mut times = Vec::new();
            for i in 0..40 {
                tokio::time::sleep(Duration::from_millis(120)).await;
                let before = viewer.text.matches('Q').count();
                let t = std::time::Instant::now();
                viewer.type_line("Q");
                tokio::time::timeout(Duration::from_secs(5), async {
                    while viewer.text.matches('Q').count() <= before {
                        let value = viewer.stream.next().await.unwrap();
                        if let TerminalEvent::Data { data, .. } =
                            serde_json::from_value(value).unwrap()
                        {
                            viewer.text.push_str(&String::from_utf8_lossy(
                                &base64::Engine::decode(
                                    &base64::engine::general_purpose::STANDARD,
                                    data,
                                )
                                .unwrap(),
                            ));
                        }
                    }
                })
                .await
                .unwrap();
                times.push(t.elapsed().as_secs_f64() * 1000.0);
                if i % 10 == 9 {
                    viewer.type_line("\u{15}");
                }
            }
            times.sort_by(|a, b| a.partial_cmp(b).unwrap());
            eprintln!(
                "ECHO {label} p50={:.2} p90={:.2} min={:.2}",
                times[20], times[36], times[0]
            );
        }
        stop.cancel();
    }
}
