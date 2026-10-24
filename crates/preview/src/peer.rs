//! Authenticated signaling supplies SDP/DTLS fingerprints. Preview bytes only
//! use the resulting reliable, ordered DataChannel; there is no edge byte relay.
use crate::mux::{Connector, Mux, PeerScoped, Stream, Transport};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, Notify, mpsc, watch};
use tokio_util::sync::CancellationToken;
use webrtc::{
    data_channel::{DataChannel, DataChannelEvent},
    peer_connection::{
        PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfigurationBuilder,
        RTCIceCandidateType, RTCIceGatheringState, RTCIceServer, RTCPeerConnectionIceEvent,
        RTCPeerConnectionState, RTCSessionDescription, SettingEngine,
    },
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Signal {
    pub kind: String,
    pub session: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sdp: Option<RTCSessionDescription>,
}
#[derive(Clone)]
pub struct OutgoingSignal {
    pub to: String,
    pub signal: Signal,
}
struct Peer {
    session: String,
    pc: Arc<dyn PeerConnection>,
    ready: watch::Receiver<Option<Mux>>,
    gathered: watch::Receiver<bool>,
    stop: CancellationToken,
}
impl Peer {
    async fn close(&self) {
        self.stop.cancel();
        let _ = self.pc.close().await;
    }
}
struct Inner {
    device: String,
    connector: Arc<dyn Connector>,
    peers: Mutex<HashMap<String, Arc<Peer>>>,
    requested: Mutex<HashMap<String, tokio::time::Instant>>,
    output: mpsc::Sender<OutgoingSignal>,
    changed: Arc<Notify>,
    stop: CancellationToken,
    ice_servers: Vec<RTCIceServer>,
    /// Local sockets ICE gathers host candidates from.
    udp_addrs: Vec<String>,
}
const PAIR_INTERVAL: Duration = Duration::from_secs(10);
/// Longest a description waits on STUN before going out with the
/// candidates it has. Signaling is not trickled, so every pairing pays this
/// on both sides when STUN doesn't answer (a network that blocks it): host
/// candidates are ready at once, and a STUN answer that hasn't come in a
/// second rarely comes at all.
const GATHER_BUDGET: Duration = Duration::from_secs(1);
#[derive(Clone)]
pub struct Peers(Arc<Inner>);
impl Peers {
    pub fn new(
        device: String,
        connector: Arc<dyn Connector>,
        stop: CancellationToken,
    ) -> (Self, mpsc::Receiver<OutgoingSignal>) {
        let (output, receiver) = mpsc::channel(64);
        (
            Self(Arc::new(Inner {
                device,
                connector,
                peers: Mutex::new(HashMap::new()),
                requested: Mutex::new(HashMap::new()),
                output,
                changed: Arc::new(Notify::new()),
                stop,
                ice_servers: vec![RTCIceServer {
                    urls: vec![
                        "stun:stun.l.google.com:19302".into(),
                        "stun:stun1.l.google.com:19302".into(),
                    ],
                    ..Default::default()
                }],
                udp_addrs: vec!["0.0.0.0:0".into()],
            })),
            receiver,
        )
    }
    /// Replace STUN servers; an empty list pairs over host candidates only.
    /// Must be called before the handle is shared.
    pub fn set_ice_servers(&mut self, urls: Vec<String>) -> anyhow::Result<()> {
        let inner = Arc::get_mut(&mut self.0).context("preview peers already shared")?;
        inner.ice_servers = if urls.is_empty() {
            Vec::new()
        } else {
            vec![RTCIceServer {
                urls,
                ..Default::default()
            }]
        };
        Ok(())
    }
    /// Bind ICE to these local addresses (default: every interface). Tests
    /// pair over loopback, which a host firewall doesn't filter. Must be
    /// called before the handle is shared.
    pub fn set_udp_addrs(&mut self, addrs: Vec<String>) -> anyhow::Result<()> {
        let inner = Arc::get_mut(&mut self.0).context("preview peers already shared")?;
        inner.udp_addrs = addrs;
        Ok(())
    }
    async fn send(&self, to: &str, signal: Signal) -> anyhow::Result<()> {
        tokio::select! { _ = self.0.stop.cancelled() => anyhow::bail!("preview networking stopped"), result = self.0.output.send(OutgoingSignal { to: to.into(), signal }) => { result?; Ok(()) } }
    }
    async fn create(
        &self,
        device: &str,
        session: String,
        initiator: bool,
    ) -> anyhow::Result<Arc<Peer>> {
        tracing::debug!(initiator, "creating preview peer");
        let (ready, receiver) = watch::channel(None);
        let (gathered, gathering) = watch::channel(false);
        let stop = self.0.stop.child_token();
        let handler = Arc::new(Handler {
            ready,
            gathered,
            channel_claimed: AtomicBool::new(false),
            // Streams this peer opens are attributed to its authenticated
            // device (sign-in callbacks only serve the device that asked).
            connector: Arc::new(PeerScoped::new(device, self.0.connector.clone())),
            initiator,
            stop: stop.clone(),
            changed: self.0.changed.clone(),
        });
        let pc: Arc<dyn PeerConnection> = Arc::new(
            PeerConnectionBuilder::new()
                .with_setting_engine(setting_engine())
                .with_configuration(
                    RTCConfigurationBuilder::new()
                        .with_ice_servers(self.0.ice_servers.clone())
                        .build(),
                )
                .with_handler(handler.clone())
                .with_udp_addrs(self.0.udp_addrs.clone())
                .with_data_channel_send_buffer_limit(256 * 1024)
                .with_sctp_receive_buffer_size(512 * 1024)
                .build()
                .await?,
        );
        if initiator {
            let channel = pc.create_data_channel("zeron-preview-v1", None).await?;
            handler.attach(channel);
        }
        tracing::debug!(initiator, "created preview peer");
        Ok(Arc::new(Peer {
            session,
            pc,
            ready: receiver,
            gathered: gathering,
            stop,
        }))
    }
    async fn description(&self, peer: &Peer, offer: bool) -> anyhow::Result<RTCSessionDescription> {
        tracing::debug!(offer, "creating preview description");
        let description = if offer {
            peer.pc.create_offer(None).await?
        } else {
            peer.pc.create_answer(None).await?
        };
        peer.pc.set_local_description(description).await?;
        tracing::debug!(offer, "gathering preview candidates");
        // Include candidates gathered so far. Gathering counts as done at the
        // first server-reflexive candidate (the address another network can
        // reach; the other STUN servers would only repeat it) or when it
        // completes. A STUN probe on an unreachable interface/server may
        // never report Complete; that must not discard usable host or
        // server-reflexive candidates from the other probes.
        let mut gathered = peer.gathered.clone();
        match tokio::time::timeout(GATHER_BUDGET, gathered.wait_for(|done| *done)).await {
            Ok(result) => {
                result.context("preview candidate gathering stopped")?;
            }
            Err(_) => {
                tracing::warn!("preview STUN discovery incomplete; trying available ICE candidates")
            }
        }
        let description = peer
            .pc
            .local_description()
            .await
            .context("missing local preview SDP")?;
        tracing::debug!(offer, "sending preview description");
        anyhow::ensure!(
            description.sdp.contains("a=candidate:"),
            "No local preview connection candidates are available"
        );
        Ok(description)
    }
    /// `requested` marks an offer made because the other device asked for one.
    async fn offer(&self, device: &str, session: String, requested: bool) -> anyhow::Result<()> {
        let mut peers = self.0.peers.lock().await;
        if let Some(existing) = peers.get(device) {
            // A live peer is reused, unless the other device asked to pair
            // under a new session: it has dropped its side, so ours is a
            // zombie whose failure would only surface once ICE consent
            // expires (~30s), stalling every request until then.
            if !existing.stop.is_cancelled() && (!requested || existing.session == session) {
                return Ok(());
            }
        }
        anyhow::ensure!(
            peers.len() < 16 || peers.contains_key(device),
            "too many preview peers"
        );
        let peer = self.create(device, session, true).await?;
        if let Some(old) = peers.insert(device.into(), peer.clone()) {
            old.close().await;
        }
        drop(peers);
        let result = async {
            let sdp = self.description(&peer, true).await?;
            self.send(
                device,
                Signal {
                    kind: "offer".into(),
                    session: peer.session.clone(),
                    sdp: Some(sdp),
                },
            )
            .await
        }
        .await;
        if result.is_err() {
            peer.close().await;
        }
        result
    }
    /// Called only for messages stamped by the authenticated coordinator.
    pub async fn signal(&self, device: &str, signal: Signal) -> anyhow::Result<()> {
        anyhow::ensure!(
            device != self.0.device && !signal.session.is_empty() && signal.session.len() <= 128,
            "invalid preview pairing"
        );
        match signal.kind.as_str() {
            "connect" => {
                anyhow::ensure!(
                    self.0.device.as_str() < device && signal.sdp.is_none(),
                    "invalid preview initiator"
                );
                self.offer(device, signal.session, true).await?;
            }
            "offer" => {
                anyhow::ensure!(
                    device < self.0.device.as_str(),
                    "invalid preview offer sender"
                );
                let mut peers = self.0.peers.lock().await;
                anyhow::ensure!(
                    peers.len() < 16 || peers.contains_key(device),
                    "too many preview peers"
                );
                let peer = self.create(device, signal.session, false).await?;
                if let Some(old) = peers.insert(device.into(), peer.clone()) {
                    old.close().await;
                }
                drop(peers);
                let result = async {
                    peer.pc
                        .set_remote_description(signal.sdp.context("missing offer")?)
                        .await?;
                    let sdp = self.description(&peer, false).await?;
                    self.send(
                        device,
                        Signal {
                            kind: "answer".into(),
                            session: peer.session.clone(),
                            sdp: Some(sdp),
                        },
                    )
                    .await
                }
                .await;
                if result.is_err() {
                    peer.close().await;
                }
                result?;
            }
            "answer" => {
                anyhow::ensure!(
                    self.0.device.as_str() < device,
                    "invalid preview answer sender"
                );
                let peer = self
                    .0
                    .peers
                    .lock()
                    .await
                    .get(device)
                    .cloned()
                    .context("unexpected preview answer")?;
                anyhow::ensure!(peer.session == signal.session, "stale preview answer");
                peer.pc
                    .set_remote_description(signal.sdp.context("missing answer")?)
                    .await?;
            }
            _ => anyhow::bail!("unknown preview signaling message"),
        }
        self.0.changed.notify_waiters();
        Ok(())
    }
    pub async fn open(
        &self,
        device: &str,
        service: &str,
        websocket: bool,
    ) -> anyhow::Result<Stream> {
        self.connect(device).await?.open(service, websocket).await
    }
    /// The live connection to `device`, pairing first when there is none.
    pub async fn connect(&self, device: &str) -> anyhow::Result<Mux> {
        let device = device.to_owned();
        let result = tokio::time::timeout(Duration::from_secs(30), async {
            // A failed or transport-dead peer is closed here; the loop below
            // pairs afresh.
            let stale = self.0.peers.lock().await.get(&device).cloned().filter(|p| {
                p.stop.is_cancelled() || p.ready.borrow().as_ref().is_some_and(Mux::is_closed)
            });
            if let Some(peer) = stale { peer.close().await; }
            loop {
                let changed = self.0.changed.notified();
                if let Some(mux) = self.ready(&device).await { return Ok(mux); }
                // Pairing is re-attempted while waiting, so one lost signal or
                // a stalled attempt costs at most the pacing interval rather
                // than the whole timeout.
                self.pair(&device).await?;
                tokio::select! { _ = self.0.stop.cancelled() => anyhow::bail!("preview networking stopped"), _ = changed => {}, _ = tokio::time::sleep(Duration::from_millis(100)) => {} }
            }
        }).await;
        if result.is_err() {
            self.remove(&device).await;
        }
        result.context("Could not connect directly to this device. Check that both devices are online and their networks allow WebRTC.")?
    }
    /// The open connection to `device`, without pairing.
    pub async fn ready(&self, device: &str) -> Option<Mux> {
        let mux = self
            .0
            .peers
            .lock()
            .await
            .get(device)
            .and_then(|peer| peer.ready.borrow().clone());
        mux.filter(|m| !m.is_closed())
    }
    /// Starts (or asks the initiating device to start) a pairing attempt, at
    /// most once per pacing interval per device. `remove` resets the pacing.
    async fn pair(&self, device: &str) -> anyhow::Result<()> {
        {
            let mut requested = self.0.requested.lock().await;
            let now = tokio::time::Instant::now();
            if requested
                .get(device)
                .is_some_and(|at| now.duration_since(*at) < PAIR_INTERVAL)
            {
                return Ok(());
            }
            requested.insert(device.to_owned(), now);
        }
        let session = uuid::Uuid::new_v4().to_string();
        if self.0.device.as_str() < device {
            self.offer(device, session, false).await
        } else {
            self.send(
                device,
                Signal {
                    kind: "connect".into(),
                    session,
                    sdp: None,
                },
            )
            .await
        }
    }
    pub async fn remove(&self, device: &str) {
        self.0.requested.lock().await.remove(device);
        if let Some(peer) = self.0.peers.lock().await.remove(device) {
            peer.close().await;
        }
        self.0.changed.notify_waiters();
    }
    pub async fn clear(&self) {
        self.0.requested.lock().await.clear();
        let peers = std::mem::take(&mut *self.0.peers.lock().await);
        for (_, peer) in peers {
            peer.close().await;
        }
        self.0.changed.notify_waiters();
    }
}
/// ICE tuned for pairing fast — a person is waiting on it (a terminal
/// screen, a preview page):
/// - checks every 50ms (RFC 8445's Ta; the library's default is 200ms),
///   up to 20 per pair, so a lossy path still gets ~1s of attempts;
/// - a working server-reflexive or peer-reflexive pair (a phone on cellular,
///   another network) is nominated after 100ms / 150ms instead of 500ms /
///   1s. Those waits only let a better pair win, and a host pair on a shared
///   network answers in a millisecond or two.
fn setting_engine() -> SettingEngine {
    let mut setting = SettingEngine::default();
    setting.set_ice_connection_attempts(Some(Duration::from_millis(50)), Some(20));
    setting.set_srflx_acceptance_min_wait(Some(Duration::from_millis(100)));
    setting.set_prflx_acceptance_min_wait(Some(Duration::from_millis(150)));
    setting
}

struct Handler {
    channel_claimed: AtomicBool,
    ready: watch::Sender<Option<Mux>>,
    gathered: watch::Sender<bool>,
    connector: Arc<dyn Connector>,
    initiator: bool,
    stop: CancellationToken,
    changed: Arc<Notify>,
}
impl Handler {
    fn attach(&self, channel: Arc<dyn DataChannel>) {
        if self.channel_claimed.swap(true, Ordering::SeqCst) {
            tokio::spawn(async move {
                let _ = channel.close().await;
            });
            return;
        }
        let ready = self.ready.clone();
        let connector = self.connector.clone();
        let stop = self.stop.clone();
        let initiator = self.initiator;
        let changed = self.changed.clone();
        tokio::spawn(async move {
            let valid = channel.label().await.is_ok_and(|s| s == "zeron-preview-v1")
                && channel.ordered().await.unwrap_or(false)
                && channel.max_retransmits().await.is_ok_and(|v| v.is_none())
                && channel
                    .max_packet_life_time()
                    .await
                    .is_ok_and(|v| v.is_none());
            if !valid {
                let _ = channel.close().await;
                return;
            }
            loop {
                let event = tokio::select! { _ = stop.cancelled() => return, event = channel.poll() => event };
                match event {
                    Some(DataChannelEvent::OnOpen) => {
                        if ready.borrow().is_some() {
                            let _ = channel.close().await;
                            return;
                        }
                        let mux = Mux::start(
                            Arc::new(ChannelTransport(channel)),
                            connector,
                            initiator,
                            stop.child_token(),
                        );
                        ready.send_replace(Some(mux));
                        changed.notify_waiters();
                        return;
                    }
                    None | Some(DataChannelEvent::OnClose) => return,
                    _ => {}
                }
            }
        });
    }
}
#[async_trait::async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            self.gathered.send_replace(true);
        }
    }
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        if event.candidate.typ == RTCIceCandidateType::Srflx {
            self.gathered.send_replace(true);
        }
    }
    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        if matches!(
            state,
            RTCPeerConnectionState::Failed
                | RTCPeerConnectionState::Closed
                | RTCPeerConnectionState::Disconnected
        ) {
            self.stop.cancel();
            self.ready.send_replace(None);
            self.changed.notify_waiters();
        }
    }
    async fn on_data_channel(&self, channel: Arc<dyn DataChannel>) {
        self.attach(channel);
    }
}
struct ChannelTransport(Arc<dyn DataChannel>);
#[async_trait::async_trait]
impl Transport for ChannelTransport {
    async fn send(&self, bytes: &[u8]) -> anyhow::Result<()> {
        self.0.send(bytes::BytesMut::from(bytes)).await?;
        Ok(())
    }
    async fn receive(&self) -> anyhow::Result<Vec<u8>> {
        loop {
            match self.0.poll().await {
                Some(DataChannelEvent::OnMessage(message)) => {
                    anyhow::ensure!(
                        message.data.len() <= 8197,
                        "oversized preview DataChannel frame"
                    );
                    return Ok(message.data.to_vec());
                }
                None | Some(DataChannelEvent::OnClose) => {
                    anyhow::bail!("preview DataChannel closed")
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    struct Echo;
    #[async_trait::async_trait]
    impl Connector for Echo {
        async fn connect(&self, id: &str) -> anyhow::Result<crate::mux::BoxIo> {
            anyhow::ensure!(id == "service", "unknown service");
            let (client, server) = tokio::io::duplex(65536);
            tokio::spawn(async move {
                let (mut r, mut w) = tokio::io::split(server);
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
            Ok(Box::new(client))
        }
    }
    #[tokio::test]
    async fn unreachable_stun_does_not_block_usable_peer_candidates() {
        // Accept UDP without answering: gathering cannot complete, although
        // both peers have usable host candidates. This reproduces a laptop
        // whose STUN requests time out while its remote host is reachable.
        let blackhole = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let stop = CancellationToken::new();
        let (mut a, mut a_out) = Peers::new("a".into(), Arc::new(Echo), stop.clone());
        let (mut b, mut b_out) = Peers::new("b".into(), Arc::new(Echo), stop.clone());
        // Loopback: a host firewall (macOS's blocks unsigned test binaries)
        // never filters it.
        a.set_udp_addrs(vec!["127.0.0.1:0".into()]).unwrap();
        b.set_udp_addrs(vec!["127.0.0.1:0".into()]).unwrap();
        let servers = vec![RTCIceServer {
            urls: vec![format!("stun:{}", blackhole.local_addr().unwrap())],
            ..Default::default()
        }];
        Arc::get_mut(&mut a.0).unwrap().ice_servers = servers.clone();
        Arc::get_mut(&mut b.0).unwrap().ice_servers = servers;
        let peer_b = b.clone();
        let forward_a = tokio::spawn(async move {
            while let Some(message) = a_out.recv().await {
                peer_b.signal("a", message.signal).await.unwrap();
            }
        });
        let peer_a = a.clone();
        let forward_b = tokio::spawn(async move {
            while let Some(message) = b_out.recv().await {
                peer_a.signal("b", message.signal).await.unwrap();
            }
        });
        let result = tokio::time::timeout(Duration::from_secs(10), async {
            let mut stream = b.open("a", "service", false).await.unwrap();
            stream
                .write_all(b"preview through available candidates")
                .await
                .unwrap();
            stream.shutdown().await.unwrap();
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).await.unwrap();
            assert_eq!(bytes, b"preview through available candidates");
        })
        .await;
        stop.cancel();
        a.clear().await;
        b.clear().await;
        forward_a.abort();
        forward_b.abort();
        result.expect("a failed STUN probe must not prevent peer setup");
    }
    async fn echo_round(peers: &Peers, device: &str) {
        let mut stream = peers.open(device, "service", false).await.unwrap();
        stream.write_all(b"ping").await.unwrap();
        stream.shutdown().await.unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).await.unwrap();
        assert_eq!(bytes, b"ping");
    }
    #[tokio::test]
    async fn a_dropped_peer_pairs_again_without_waiting_for_ice_failure() {
        // The device that drops its peer (sleep, network roam, coordinator
        // lease reset) must be served promptly by the other side, whose own
        // peer object still looks healthy until ICE consent expires.
        let stop = CancellationToken::new();
        let (mut a, mut a_out) = Peers::new("a".into(), Arc::new(Echo), stop.clone());
        let (mut b, mut b_out) = Peers::new("b".into(), Arc::new(Echo), stop.clone());
        // Loopback: a host firewall (macOS's blocks unsigned test binaries)
        // never filters it.
        a.set_udp_addrs(vec!["127.0.0.1:0".into()]).unwrap();
        b.set_udp_addrs(vec!["127.0.0.1:0".into()]).unwrap();
        a.set_ice_servers(Vec::new()).unwrap();
        b.set_ice_servers(Vec::new()).unwrap();
        let peer_b = b.clone();
        let forward_a = tokio::spawn(async move {
            while let Some(message) = a_out.recv().await {
                let _ = peer_b.signal("a", message.signal).await;
            }
        });
        let peer_a = a.clone();
        let forward_b = tokio::spawn(async move {
            while let Some(message) = b_out.recv().await {
                let _ = peer_a.signal("b", message.signal).await;
            }
        });
        let result = tokio::time::timeout(Duration::from_secs(40), async {
            echo_round(&b, "a").await;
            // Non-initiator drops: its connect request must replace a's peer.
            b.remove("a").await;
            tokio::time::timeout(Duration::from_secs(8), echo_round(&b, "a"))
                .await
                .expect("re-pair after the requester dropped its peer stalled");
            // Initiator drops: it offers again and b replaces its peer.
            a.remove("b").await;
            tokio::time::timeout(Duration::from_secs(8), echo_round(&a, "b"))
                .await
                .expect("re-pair after the initiator dropped its peer stalled");
            // Both directions still work over the final pair.
            echo_round(&b, "a").await;
        })
        .await;
        stop.cancel();
        a.clear().await;
        b.clear().await;
        forward_a.abort();
        forward_b.abort();
        result.expect("re-pairing test stalled");
    }
    #[tokio::test]
    async fn actual_webrtc_pair_streams_in_both_directions() {
        let stop = CancellationToken::new();
        let (mut a, mut a_out) = Peers::new("a".into(), Arc::new(Echo), stop.clone());
        let (mut b, mut b_out) = Peers::new("b".into(), Arc::new(Echo), stop.clone());
        // Loopback: a host firewall (macOS's blocks unsigned test binaries)
        // never filters it.
        a.set_udp_addrs(vec!["127.0.0.1:0".into()]).unwrap();
        b.set_udp_addrs(vec!["127.0.0.1:0".into()]).unwrap();
        Arc::get_mut(&mut a.0).unwrap().ice_servers.clear();
        Arc::get_mut(&mut b.0).unwrap().ice_servers.clear();
        let peer_b = b.clone();
        let forward_a = tokio::spawn(async move {
            while let Some(message) = a_out.recv().await {
                peer_b.signal("a", message.signal).await.unwrap();
            }
        });
        let peer_a = a.clone();
        let forward_b = tokio::spawn(async move {
            while let Some(message) = b_out.recv().await {
                peer_a.signal("b", message.signal).await.unwrap();
            }
        });
        // A page's initial burst must pair once, rather than flood signaling
        // with one connection request per asset.
        futures::future::join_all((0..8).map(|_| async {
            let mut stream = b.open("a", "service", false).await.unwrap();
            stream.write_all(b"asset").await.unwrap();
            stream.shutdown().await.unwrap();
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).await.unwrap();
            assert_eq!(bytes, b"asset");
        }))
        .await;
        for (from, to) in [(&b, "a"), (&a, "b")] {
            let stream = from.open(to, "service", true).await.unwrap();
            let (mut read, mut write) = tokio::io::split(stream);
            let expected = vec![42; 1024 * 1024 + 3];
            let bytes = expected.clone();
            tokio::time::timeout(Duration::from_secs(15), async {
                let sender = async {
                    write.write_all(&bytes).await.unwrap();
                    write.shutdown().await.unwrap();
                };
                let receiver = async {
                    let mut received = Vec::new();
                    read.read_to_end(&mut received).await.unwrap();
                    assert_eq!(received.len(), expected.len());
                    assert!(received == expected);
                };
                tokio::join!(sender, receiver);
            })
            .await
            .expect("P2P stream stalled");
        }
        stop.cancel();
        a.clear().await;
        b.clear().await;
        forward_a.abort();
        forward_b.abort();
    }

    /// The same round trip with the answering peer in another process
    /// (signals relayed over the child's stdin/stdout), as phone and host are.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "benchmark"]
    async fn bench_cross_process_round_trip() {
        use tokio::io::AsyncBufReadExt;
        let stop = CancellationToken::new();
        let child_role = std::env::var("BENCH_CHILD").is_ok();
        let me = if child_role { "a" } else { "b" };
        let (mut peers, mut out) = Peers::new(me.into(), Arc::new(Echo), stop.clone());
        peers.set_ice_servers(Vec::new()).unwrap();
        let udp = std::env::var("BENCH_UDP").unwrap_or_else(|_| "127.0.0.1:0".into());
        peers.set_udp_addrs(vec![udp]).unwrap();
        if child_role {
            let p = peers.clone();
            tokio::spawn(async move {
                let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Some(json) = line.strip_prefix("SIG ") {
                        let _ = p.signal("b", serde_json::from_str(json).unwrap()).await;
                    }
                }
                std::process::exit(0);
            });
            while let Some(m) = out.recv().await {
                println!("SIG {}", serde_json::to_string(&m.signal).unwrap());
            }
            return;
        }
        let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "peer::tests::bench_cross_process_round_trip",
                "--nocapture",
            ])
            .env("BENCH_CHILD", "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        tokio::spawn(async move {
            while let Some(m) = out.recv().await {
                let line = format!("SIG {}\n", serde_json::to_string(&m.signal).unwrap());
                stdin.write_all(line.as_bytes()).await.unwrap();
            }
        });
        let p = peers.clone();
        tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(json) = line.strip_prefix("SIG ") {
                    let _ = p.signal("a", serde_json::from_str(json).unwrap()).await;
                }
            }
        });
        let mut stream = peers.open("a", "service", false).await.unwrap();
        let mut byte = [0u8; 1];
        for gap in [0u64, 120] {
            let mut times = Vec::new();
            for _ in 0..60 {
                tokio::time::sleep(Duration::from_millis(gap)).await;
                let t = std::time::Instant::now();
                stream.write_all(b"x").await.unwrap();
                stream.read_exact(&mut byte).await.unwrap();
                times.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            times.sort_by(|a, b| a.partial_cmp(b).unwrap());
            eprintln!(
                "XRTT gap={gap}ms p50={:.2} p90={:.2} min={:.2} max={:.2}",
                times[30], times[54], times[0], times[59]
            );
        }
        stop.cancel();
    }

    /// Round trips of one small frame over a real pair (loopback): what the
    /// DataChannel itself adds to every keystroke.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "benchmark"]
    async fn bench_small_frame_round_trip() {
        let stop = CancellationToken::new();
        let (mut a, mut a_out) = Peers::new("a".into(), Arc::new(Echo), stop.clone());
        let (mut b, mut b_out) = Peers::new("b".into(), Arc::new(Echo), stop.clone());
        a.set_ice_servers(Vec::new()).unwrap();
        b.set_ice_servers(Vec::new()).unwrap();
        a.set_udp_addrs(vec!["127.0.0.1:0".into()]).unwrap();
        b.set_udp_addrs(vec!["127.0.0.1:0".into()]).unwrap();
        let peer_b = b.clone();
        tokio::spawn(async move {
            while let Some(m) = a_out.recv().await {
                let _ = peer_b.signal("a", m.signal).await;
            }
        });
        let peer_a = a.clone();
        tokio::spawn(async move {
            while let Some(m) = b_out.recv().await {
                let _ = peer_a.signal("b", m.signal).await;
            }
        });
        let paired = std::time::Instant::now();
        let mut stream = b.open("a", "service", false).await.unwrap();
        eprintln!("PAIR+OPEN {:?}", paired.elapsed());
        let mut byte = [0u8; 1];
        // Back to back, then with a typist's pauses between keys.
        for gap in [0u64, 120] {
            let mut times = Vec::new();
            for _ in 0..60 {
                tokio::time::sleep(Duration::from_millis(gap)).await;
                let t = std::time::Instant::now();
                stream.write_all(b"x").await.unwrap();
                stream.read_exact(&mut byte).await.unwrap();
                times.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            times.sort_by(|a, b| a.partial_cmp(b).unwrap());
            eprintln!(
                "RTT gap={gap}ms p50={:.2} p90={:.2} min={:.2} max={:.2}",
                times[30], times[54], times[0], times[59]
            );
        }
        stop.cancel();
    }
}
