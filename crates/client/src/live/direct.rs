//! Direct connections from this device to the user's other devices: the
//! same WebRTC peers and signaling every engine runs (`zeron-preview`), with
//! this device as a pure viewer. Terminals ride them when they can (see
//! [`crate::terminal`]); everything else stays on the relay.

use std::sync::Arc;

use zeron_preview::viewer::ViewerPeers;

use super::Bearer;
use crate::client::ClientInner;

#[async_trait::async_trait]
impl zeron_preview::signaling::TokenSource for Bearer {
    async fn token(&self) -> anyhow::Result<String> {
        self.get().await.map_err(|e| anyhow::anyhow!("{e}"))
    }
}

/// Join signaling as this device. `None` when it can't start (a broken
/// data dir): terminals then stay on the relay.
pub(crate) fn start(inner: &Arc<ClientInner>) -> Option<ViewerPeers> {
    let config = zeron_preview::signaling::Config {
        edge_url: inner.config.edge_base().to_owned(),
        org_id: inner.credentials.org_id().to_owned(),
        tokens: Arc::new(Bearer(Arc::downgrade(inner))),
    };
    match ViewerPeers::start(
        &crate::runtime::handle(),
        inner.config.device_id.clone(),
        inner.config.device_name.clone(),
        inner.config.data_dir.join("direct-peers.json"),
        config,
    ) {
        Ok(peers) => Some(peers),
        Err(error) => {
            tracing::warn!(%error, "direct connections unavailable");
            None
        }
    }
}
