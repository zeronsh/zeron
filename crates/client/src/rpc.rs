//! Host RPC vocabulary: what a viewer asks an execution host over the
//! device-room relay (catalogs, refs, folders, uploads, queue actions), and
//! the capability gates that decide whether to ask at all.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;
pub use zeron_proto::{FolderEntry, FolderListing, RepoRef, WorktreeSpec};
pub use zeron_rpc::RpcSubscription;

/// Engine capability strings a device row advertises (`Device::capabilities`).
/// Capabilities, not semver: a personal integration build can share an
/// upstream version without the doc/RPC surface.
pub mod capability {
    pub const MESSAGE_QUEUE_V1: &str = "message-queue-v1";
    pub const MESSAGE_QUEUE_ACTIONS_V1: &str = "message-queue-actions-v1";
    pub const MESSAGE_QUEUE_ATTACHMENTS_V1: &str = "message-queue-attachments-v1";
    pub const MESSAGE_QUEUE_CLEAN_ATTACHMENT_TEXT_V1: &str =
        "message-queue-clean-attachment-text-v1";
    pub const MESSAGE_QUEUE_EDIT_LEASE_V1: &str = "message-queue-edit-lease-v1";

    /// Every queue capability (demo hosts advertise the full surface).
    pub const ALL_QUEUE: &[&str] = &[
        MESSAGE_QUEUE_V1,
        MESSAGE_QUEUE_ACTIONS_V1,
        MESSAGE_QUEUE_ATTACHMENTS_V1,
        MESSAGE_QUEUE_CLEAN_ATTACHMENT_TEXT_V1,
        MESSAGE_QUEUE_EDIT_LEASE_V1,
    ];
}

/// Queued-attachment version gate (composer.rs QUEUED_ATTACHMENTS_MIN): the
/// host must defer commands carrying `pending://` refs until the bytes land.
pub const QUEUED_ATTACHMENTS_MIN: (u64, u64, u64) = (0, 2, 12);

/// Upload progress callback: fraction in `0.0..=1.0` of the file's bytes the
/// host has committed.
pub type ProgressFn = Arc<dyn Fn(f64) + Send + Sync>;

/// Relay method names (single source of truth: `zeron_rpc::methods`).
pub mod methods {
    pub use zeron_rpc::methods::*;
}

/// Where a host stream's items go ([`crate::Client::host_watch`]). Called on
/// a client runtime thread, in order; keep it quick (hand off to the UI).
pub trait HostWatchSink: Send + Sync {
    fn item(&self, value: serde_json::Value);
    /// The stream ended without being cancelled: the host finished it (a
    /// terminal exited), the connection dropped, or the client shut down.
    /// Re-subscribe to resume (`SubscribeTerminal` with `afterSeq`).
    fn ended(&self);
}

/// A running host stream. [`Self::cancel`] — or dropping the handle — stops
/// it and cancels the host side; the sink then hears nothing more.
#[derive(Debug)]
pub struct HostWatch {
    cancel: CancellationToken,
}

impl HostWatch {
    /// Pump `stream` into `sink` on the client runtime until the host ends
    /// it, `parent` is cancelled (client shutdown → `ended`), or the handle
    /// is cancelled/dropped (silently).
    pub(crate) fn spawn(
        mut stream: RpcSubscription,
        sink: Arc<dyn HostWatchSink>,
        parent: &CancellationToken,
    ) -> Self {
        let cancel = CancellationToken::new();
        let own = cancel.clone();
        let parent = parent.clone();
        crate::runtime::shared().spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = own.cancelled() => break,
                    _ = parent.cancelled() => {
                        sink.ended();
                        break;
                    }
                    item = stream.recv() => match item {
                        Some(value) => sink.item(value),
                        None => {
                            sink.ended();
                            break;
                        }
                    },
                }
            }
            // Dropped here, on the runtime: the cancel frame goes out.
            drop(stream);
        });
        Self { cancel }
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// `false` once cancelled (the pump may still be finishing).
    pub fn is_active(&self) -> bool {
        !self.cancel.is_cancelled()
    }
}

impl Drop for HostWatch {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
