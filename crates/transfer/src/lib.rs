//! zeron-transfer — files and folders sent directly between a user's
//! engines (docs/file-transfer.md).
//!
//! A transfer is a [`manifest::Manifest`] plus 1 MiB [`blocks`] streamed
//! from disk over one control lane and several data lanes. Lanes are plain
//! ordered byte streams supplied by the engine: WebRTC mux streams when a
//! direct path pairs, [`relay`] pipes over the DeviceRoom otherwise. The
//! receiver verifies every block's SHA-256 before writing it into a hidden
//! `*.part` file, records verified blocks durably, checks each file's
//! whole-file SHA-256, and renames it into place. A dropped tunnel resumes
//! from those verified blocks.

pub mod blocks;
pub mod manifest;
mod receiver;
pub mod relay;
mod sender;
mod service;
pub mod wire;

use tokio::io::{AsyncRead, AsyncWrite};
use zeron_proto::FileTransferTransport;

pub use relay::{RelayPipes, RelayTunnel};
pub use service::{
    SendRequest, Transfers, TransfersConfig, TransportPolicy, WeakTransfers, expand_home,
};

/// Mux service id of the P2P lanes.
pub const P2P_SERVICE: &str = "file-transfer:v1";

pub trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}
pub type BoxIo = Box<dyn Io>;

/// A temporary connection to one peer engine that can open lanes.
#[async_trait::async_trait]
pub trait Tunnel: Send + Sync {
    fn transport(&self) -> FileTransferTransport;
    /// Data lanes worth opening in parallel.
    fn max_lanes(&self) -> usize;
    async fn open_lane(&self) -> anyhow::Result<BoxIo>;
    /// The transfer is done with this tunnel (P2P: release it when idle).
    async fn close(&self);
}

/// What the transfer service needs from the engine around it.
#[async_trait::async_trait]
pub trait Network: Send + Sync + 'static {
    /// Open a tunnel to `device`: P2P first unless `policy` forbids it, the
    /// relay when no direct path pairs in time.
    async fn connect(
        &self,
        device: &str,
        policy: TransportPolicy,
    ) -> anyhow::Result<Box<dyn Tunnel>>;
    /// Best effort: tell `device` that this side cancelled `transfer_id`
    /// while no tunnel was up.
    async fn notify_cancel(&self, device: &str, transfer_id: &str);
    /// Display name of a device of this account.
    fn device_name(&self, device: &str) -> Option<String>;
    /// Folders (besides the home folder) a sender may name as destination.
    fn destination_roots(&self) -> Vec<std::path::PathBuf>;
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}
