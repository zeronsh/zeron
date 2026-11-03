//! Direct connections for a device that only reaches out — a phone. It joins
//! the same signaling as every engine (advertising no services) and pairs
//! with the user's other devices on demand; anything a peer tries to open on
//! it is refused.
//!
//! A phone's network changes under it (Wi-Fi to cellular, a dead zone) and
//! the OS suspends it in the background. [`ViewerPeers::network_changed`] and
//! [`ViewerPeers::foreground`] drop the connections that can't have survived
//! (their streams end at once, so whatever rides them falls back without
//! waiting for silence detection), redial signaling, and tell the users of
//! [`ViewerPeers::changed`] to try the direct path again now.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Notify, watch};
use tokio_util::sync::CancellationToken;

use crate::catalog::Catalog;
use crate::mux::{BoxIo, Connector, Stream};
use crate::peer::Peers;
use crate::signaling;

/// Back from the background after this long, a suspended phone's
/// connections are presumed dead (NAT bindings and ICE consent lapse).
const SUSPENSION_SURVIVES: Duration = Duration::from_secs(8);

/// Serves nothing: a viewer device only opens streams.
struct Refuse;

#[async_trait::async_trait]
impl Connector for Refuse {
    async fn connect(&self, _service: &str) -> anyhow::Result<BoxIo> {
        anyhow::bail!("this device doesn't serve streams")
    }
}

#[derive(Clone)]
pub struct ViewerPeers {
    /// Calls arrive from platform threads: background work goes here.
    runtime: tokio::runtime::Handle,
    peers: Peers,
    wake: Arc<Notify>,
    changes: Arc<watch::Sender<u64>>,
    stop: CancellationToken,
}

impl ViewerPeers {
    /// Join signaling as `device_id` on `runtime`. `catalog_file` keeps the
    /// (unused) preview names a catalog needs.
    pub fn start(
        runtime: &tokio::runtime::Handle,
        device_id: String,
        device_name: String,
        catalog_file: PathBuf,
        signaling: signaling::Config,
    ) -> anyhow::Result<Self> {
        let catalog = Catalog::open(catalog_file, device_id.clone(), device_name)?;
        let stop = CancellationToken::new();
        let (peers, outgoing) = {
            let _entered = runtime.enter();
            Peers::new(device_id, Arc::new(Refuse), stop.child_token())
        };
        let wake = Arc::new(Notify::new());
        runtime.spawn(signaling::run_with_wake(
            signaling,
            catalog,
            peers.clone(),
            outgoing,
            stop.child_token(),
            wake.clone(),
        ));
        Ok(Self {
            runtime: runtime.clone(),
            peers,
            wake,
            changes: Arc::new(watch::channel(0).0),
            stop,
        })
    }

    /// A direct connection to `device` is open right now.
    pub async fn ready(&self, device: &str) -> bool {
        self.peers.ready(device).await.is_some()
    }

    /// Open `service` on `device`, pairing first when needed (bounded).
    pub async fn open(&self, device: &str, service: &str) -> anyhow::Result<Stream> {
        self.peers.open(device, service, false).await
    }

    /// Pair with `device` in the background, so a stream opened shortly
    /// after finds the connection up.
    pub fn warm(&self, device: &str) {
        let peers = self.peers.clone();
        let device = device.to_owned();
        self.runtime.spawn(async move {
            if let Err(error) = peers.connect(&device).await {
                tracing::debug!(%error, "direct connection warm-up failed");
            }
        });
    }

    /// The device's network changed: every connection's path is gone.
    pub fn network_changed(&self) {
        let peers = self.peers.clone();
        self.runtime.spawn(async move { peers.clear().await });
        self.wake.notify_one();
        self.changes.send_modify(|n| *n += 1);
    }

    /// Back in front after `away` in the background.
    pub fn foreground(&self, away: Duration) {
        if away >= SUSPENSION_SURVIVES {
            let peers = self.peers.clone();
            self.runtime.spawn(async move { peers.clear().await });
        }
        self.wake.notify_one();
        self.changes.send_modify(|n| *n += 1);
    }

    /// Resolves at the next network change or return to the foreground.
    pub async fn changed(&self) {
        let mut changes = self.changes.subscribe();
        let _ = changes.changed().await;
    }

    pub fn stop(&self) {
        self.stop.cancel();
    }
}
