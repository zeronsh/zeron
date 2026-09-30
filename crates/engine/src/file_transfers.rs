//! Device-to-device file transfer (docs/file-transfer.md): the engine's
//! side of `zeron-transfer`. Tunnels are the preview service's WebRTC mux
//! (P2P, paired through the same edge signaling) or, when no direct path
//! pairs within [`P2P_ATTEMPT`], a relay pipe over the DeviceRoom link.
//! Peers are this account's own devices — the remote-workspace trust
//! boundary of ARCHITECTURE.md §1 — so their transfers are accepted into
//! the inbox unless this device asks for confirmation.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use zeron_doc::SessionCommandPayload;
use zeron_proto::{Device, FileTransferTransport, capabilities};
use zeron_rpc::{LinkCache, methods};
use zeron_transfer::{BoxIo, Network, P2P_SERVICE, RelayTunnel, TransportPolicy, Tunnel};

use crate::doc_host::DocHost;
use crate::workspace_host::WorkspaceHost;

/// How long a transfer waits for a direct connection before the relay.
const P2P_ATTEMPT: Duration = Duration::from_secs(8);
/// A forced-P2P transfer (tests, diagnostics) waits as long as previews do.
const P2P_ONLY_ATTEMPT: Duration = Duration::from_secs(30);
const LANE_OPEN: Duration = Duration::from_secs(15);
/// Agent-to-agent messages carry this prefix (`zeron mcp` attribution);
/// they were not typed by the user on any device.
const AGENT_MESSAGE_PREFIX: &str = "[Message from Zeron chat ";

pub(crate) struct EngineNetwork {
    device_id: String,
    previews: zeron_preview::PreviewService,
    workspace: WorkspaceHost,
    links: Arc<Mutex<Option<Arc<LinkCache>>>>,
}

impl EngineNetwork {
    pub(crate) fn new(
        device_id: String,
        previews: zeron_preview::PreviewService,
        workspace: WorkspaceHost,
        links: Arc<Mutex<Option<Arc<LinkCache>>>>,
    ) -> Self {
        Self {
            device_id,
            previews,
            workspace,
            links,
        }
    }

    fn links(&self) -> Option<Arc<LinkCache>> {
        self.links
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

struct P2pTunnel {
    previews: zeron_preview::PreviewService,
    device: String,
    first: Mutex<Option<zeron_preview::mux::Stream>>,
    /// Paired for this transfer: torn down after it.
    release: bool,
}

#[async_trait::async_trait]
impl Tunnel for P2pTunnel {
    fn transport(&self) -> FileTransferTransport {
        FileTransferTransport::P2p
    }

    /// One lane: every mux stream shares a single SCTP association, and
    /// parallel 64 KiB windows overrun its buffers (measured: 1 stream
    /// 40–130 MiB/s, 2–8 streams 8–45 MiB/s on loopback).
    fn max_lanes(&self) -> usize {
        1
    }

    async fn open_lane(&self) -> anyhow::Result<BoxIo> {
        let first = self
            .first
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let stream = match first {
            Some(stream) => stream,
            None => {
                self.previews
                    .open_peer_stream(&self.device, P2P_SERVICE, LANE_OPEN)
                    .await?
            }
        };
        Ok(Box::new(stream))
    }

    async fn close(&self) {
        if self.release {
            // Let the lanes' final frames drain before looking for idleness.
            tokio::time::sleep(Duration::from_millis(500)).await;
            self.previews.release_peer_if_idle(&self.device).await;
        }
    }
}

#[async_trait::async_trait]
impl Network for EngineNetwork {
    async fn connect(
        &self,
        device: &str,
        policy: TransportPolicy,
    ) -> anyhow::Result<Box<dyn Tunnel>> {
        if policy != TransportPolicy::Relay && self.previews.peers_available() {
            let paired = self.previews.is_paired(device).await;
            let attempt = if policy == TransportPolicy::P2p {
                P2P_ONLY_ATTEMPT
            } else {
                P2P_ATTEMPT
            };
            match self
                .previews
                .open_peer_stream(device, P2P_SERVICE, attempt)
                .await
            {
                Ok(stream) => {
                    return Ok(Box::new(P2pTunnel {
                        previews: self.previews.clone(),
                        device: device.to_owned(),
                        first: Mutex::new(Some(stream)),
                        release: !paired,
                    }));
                }
                Err(error) if policy == TransportPolicy::P2p => return Err(error),
                Err(error) => {
                    tracing::info!(%device, %error, "file transfer: no direct path; using the relay")
                }
            }
        }
        anyhow::ensure!(
            policy != TransportPolicy::P2p,
            "direct connections are unavailable (sign in to sync)"
        );
        let links = self
            .links()
            .ok_or_else(|| anyhow::anyhow!("Sign in to send files to your other devices"))?;
        let client = links.client(device).await?;
        Ok(Box::new(RelayTunnel::new(client, self.device_id.clone())))
    }

    async fn notify_cancel(&self, device: &str, transfer_id: &str) {
        let Some(links) = self.links() else {
            return;
        };
        let call = async {
            let client = links.client(device).await?;
            client
                .call(
                    methods::CANCEL_FILE_TRANSFER,
                    serde_json::json!({ "transferId": transfer_id, "fromPeer": true }),
                )
                .await?;
            anyhow::Ok(())
        };
        if let Err(error) = tokio::time::timeout(Duration::from_secs(15), call)
            .await
            .map_err(anyhow::Error::from)
            .and_then(|r| r)
        {
            tracing::debug!(%device, %error, "could not tell the other device about a cancel");
        }
    }

    fn device_name(&self, device: &str) -> Option<String> {
        self.workspace
            .read_devices()
            .ok()?
            .into_iter()
            .find(|d| d.id == device)
            .map(|d| d.name)
    }

    fn destination_roots(&self) -> Vec<PathBuf> {
        self.workspace
            .read_spaces()
            .unwrap_or_default()
            .into_iter()
            .filter(|space| space.device_id == self.device_id)
            .map(|space| PathBuf::from(space.path))
            .collect()
    }
}

/// The P2P service handler: a peer opened `file-transfer:v1` on our mux.
pub(crate) fn register_peer_service(
    previews: &zeron_preview::PreviewService,
    transfers: &zeron_transfer::Transfers,
) {
    let weak = transfers.downgrade();
    previews.register_peer_service(
        P2P_SERVICE,
        Arc::new(move |peer: String, _service: String| {
            let transfers = weak.upgrade();
            Box::pin(async move {
                let transfers =
                    transfers.ok_or_else(|| anyhow::anyhow!("file transfers stopped"))?;
                let (local, remote) = tokio::io::duplex(256 * 1024);
                transfers.accept_lane(peer, FileTransferTransport::P2p, Box::new(remote));
                Ok(Box::new(local) as zeron_preview::mux::BoxIo)
            })
        }),
    );
}

/// Who to send to when an agent says "send it to me": the device that sent
/// the chat's latest user message, when that is another engine device.
pub(crate) fn recipient_for_chat(
    doc_host: &DocHost,
    workspace: &WorkspaceHost,
    local_device: &str,
    chat_id: &str,
) -> Result<String, String> {
    let handle = doc_host.open(chat_id).map_err(|e| e.to_string())?;
    let commands = handle.doc().read_commands().map_err(|e| e.to_string())?;
    let typed = commands
        .iter()
        .filter(|command| match &command.payload {
            SessionCommandPayload::Run { request, .. } => {
                !request.prompt.starts_with(AGENT_MESSAGE_PREFIX)
            }
            SessionCommandPayload::Steer { prompt, .. } => {
                !prompt.starts_with(AGENT_MESSAGE_PREFIX)
            }
            _ => false,
        })
        .max_by_key(|command| command.issued_at);
    let devices = workspace.read_devices().unwrap_or_default();
    let name = |id: &str| {
        devices
            .iter()
            .find(|d| d.id == id)
            .map(|d| d.name.clone())
            .unwrap_or_else(|| id.to_owned())
    };
    let receivers = || {
        let names: Vec<String> = devices
            .iter()
            .filter(|d| d.id != local_device && can_receive(d))
            .map(|d| format!("{} ({})", d.name, d.id))
            .collect();
        if names.is_empty() {
            "no other device of this account can receive files".to_owned()
        } else {
            format!("devices that can receive: {}", names.join(", "))
        }
    };
    let Some(command) = typed else {
        return Err(format!(
            "This chat has no message from another device to reply to; say which device to send to ({}).",
            receivers()
        ));
    };
    let from = command.issued_by.as_str();
    if from == local_device {
        return Err(format!(
            "The latest message in this chat was typed on this device ({}), so there is no other device to send to by default; say which one ({}).",
            name(from),
            receivers()
        ));
    }
    match devices.iter().find(|d| d.id == from) {
        Some(device) if can_receive(device) => Ok(from.to_owned()),
        _ => Err(format!(
            "The latest message came from {}, which doesn't run a Zeron engine that can receive files; say which device to send to ({}).",
            name(from),
            receivers()
        )),
    }
}

/// Resolve a `toDeviceId` that may also be a device name.
pub(crate) fn resolve_device(workspace: &WorkspaceHost, key: &str) -> Result<String, String> {
    let devices = workspace.read_devices().unwrap_or_default();
    if devices.iter().any(|d| d.id == key) {
        return Ok(key.to_owned());
    }
    let matches: Vec<&Device> = devices
        .iter()
        .filter(|d| d.name.eq_ignore_ascii_case(key.trim()))
        .collect();
    match matches.as_slice() {
        [device] => Ok(device.id.clone()),
        [] => Err(format!("No device named {key}")),
        _ => Err(format!("Several devices are named {key}; use its id")),
    }
}

/// An engine that speaks the transfer protocol.
pub(crate) fn can_receive(device: &Device) -> bool {
    device.supports(capabilities::FILE_TRANSFER_V1)
}
